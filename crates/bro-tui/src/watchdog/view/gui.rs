//! The watchdog card in the desktop app (prefix w): the setup form for a session's milestones and how the
//! watchdog behaves, and, once one runs, its live status page. Keys reach [`WatchdogView`] through the
//! workspace exactly as in the terminal; this draws its state (the focused field, the caret, the error, the
//! armed stop) and turns clicks into what the terminal UI's clicks do.
//!
//! A WinUI dialog: 8px corners, the title, the body, and the buttons in a command band across the bottom. The
//! milestones are `TextArea`s, which wrap by terminal cells, so they're drawn in the terminal font and their
//! width in cells (`field_width`, which the keys move by) comes from this layout. The editor drawing here is
//! shared with the prompt editor (`watchdog::prompt_editor::gui`).

use super::{BUTTONS, Button, Field, Out, WatchdogView};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::{self, Mark};
use crate::app::gui::kit::{Cursor, Style, TextBox, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::textarea::TextArea;
use bro_bridge::{MilestoneStatus, WatchdogSnapshot};
use bro_core::Harness;
use bro_gfx::text::Align;
use bro_gfx::{Color, Corners, Point, Rect};
use crossterm::event::KeyModifiers;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

/// The watchdog's mark (the terminal UI's `ui::icon("watchdog")`).
pub(crate) const ICON: &str = "\u{F0A43}";
const CHECK: &str = "\u{F012C}";
const CHECK_CIRCLE: &str = "\u{F05E0}";
const PLAY_CIRCLE: &str = "\u{F040D}";
const CIRCLE: &str = "\u{F0766}";
const CLOSE: &str = "\u{F0156}";
const PAUSE: &str = "\u{F03E6}";
const EYE: &str = "\u{F06D0}";
const ALERT: &str = "\u{F0026}";
/// The orchestrator's star (as the launcher and the tab strip mark it).
const STAR: &str = "\u{F04CE}";
/// The terminal font's size in the editors, and the height of one visual row.
pub(crate) const MONO: f32 = 13.0;
pub(crate) const ROW_H: f32 = 20.0;
/// Dialog padding, and the title row's height.
pub(crate) const PAD: f32 = 24.0;
pub(crate) const HEAD_H: f32 = 64.0;
/// A dialog button's height (WinUI's), and the command band's padding around the buttons.
pub(crate) const BUTTON_H: f32 = 32.0;
pub(crate) const BAND_PAD: f32 = 20.0;
const WIDTH: f32 = 720.0;
/// Inside a milestone field.
const FIELD_X: f32 = 12.0;
const FIELD_Y: f32 = 8.0;
/// Room kept at a field's right for its remove button.
const REMOVE_W: f32 = 26.0;
/// An option row (label over its help, the control at the right), and how much taller it is with its
/// control under the text (narrow windows).
const OPT_H: f32 = 56.0;
const STACK_H: f32 = 42.0;
/// Narrower than this, a row's choices go under its label.
const STACK_BELOW: f32 = 620.0;
/// A control's height.
const CTRL_H: f32 = 32.0;

// click targets
const PRIMARY: usize = 2;
const CANCEL: usize = 3;
const ROW: usize = 10;
const SWITCH: usize = 20;
const BUTTON: usize = 30;
const WAKE: usize = 100;
const BUDGET: usize = 200;
const MILESTONE: usize = 1000;
const REMOVE: usize = 3000;

/// The option rows, top to bottom.
const OPTS: [Field; 4] = [Field::Wake, Field::Fresh, Field::Second, Field::Budget];

// ---------------------------------------------------------------------------------------------- shared

/// When a dialog opened (it eases in) and when it was last drawn (to notice it was closed and opened again).
#[derive(Default)]
pub(crate) struct Opened {
    view: usize,
    last: Option<Instant>,
    at: Option<Instant>,
}

impl Opened {
    /// Note a frame of the dialog drawn from `view` (its address; `new`: its state looks freshly made, as a
    /// new one at a freed one's address would); true when it has just opened.
    pub(crate) fn frame(&mut self, ui: &Ui, view: usize, new: bool) -> bool {
        let fresh = self.view != view || new;
        if fresh {
            self.view = view;
            self.at = Some(ui.now);
        }
        self.last = Some(ui.now);
        fresh
    }

    /// It wasn't drawn a moment ago (closed, or the window sat idle).
    pub(crate) fn idle(&self, ui: &Ui) -> bool {
        self.last.is_none_or(|t| ui.now.saturating_duration_since(t) > Duration::from_millis(400))
    }

    /// 0 → 1 while it opens (eased out).
    pub(crate) fn amount(&self, ui: &mut Ui) -> f32 {
        let t = self.at.map_or(1.0, |a| ui.now.saturating_duration_since(a).as_secs_f32() / 0.16).min(1.0);
        if t < 1.0 {
            ui.ticking = true;
        }
        1.0 - (1.0 - t).powi(3)
    }

    /// A key that's new each time the dialog opens (eased values start where they belong).
    pub(crate) fn key(&self) -> Option<Instant> {
        self.at
    }
}

/// The caret: solid for a moment after it moves or the text changes, then blinking.
#[derive(Default)]
pub(crate) struct Caret {
    at: (usize, usize),
    moved: Option<Instant>,
}

impl Caret {
    pub(crate) fn on(&mut self, ui: &mut Ui, cursor: usize, len: usize) -> bool {
        ui.ticking = true;
        if self.at != (cursor, len) || self.moved.is_none() {
            self.at = (cursor, len);
            self.moved = Some(ui.now);
        }
        let t = self.moved.map_or(0.0, |m| ui.now.saturating_duration_since(m).as_secs_f32());
        t < 0.6 || ((t - 0.6) * 1.6).fract() < 0.55
    }
}

/// Where an editor's text sits on screen, so a click lands on the character under the pointer.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Geo {
    /// top-left of visual row 0 (above the view when scrolled)
    pub origin: Point,
    /// the wrap width in cells
    pub width: usize,
    /// one cell, logical px
    pub adv: f32,
    /// the rows in view (a click in the padding lands on the nearest one)
    pub view: Rect,
}

/// One cell of the terminal font at `size`, as the layout engine draws it (rows wrapped by cell count fit
/// what's drawn).
pub(crate) fn mono_advance(ui: &mut Ui, size: f32) -> f32 {
    const PROBE: &str = "0000000000000000000000000000000000000000";
    (ui.measure(PROBE, Style::mono(size, ui.t.text)) / PROBE.len() as f32).max(1.0)
}

/// The visual row holding byte `cur` (as the TextArea counts it: the later row where two meet).
pub(crate) fn cursor_row(rows: &[(usize, usize)], cur: usize) -> usize {
    rows.iter().rposition(|&(s, e)| cur >= s && cur <= e).unwrap_or(0)
}

/// Draw `ta`'s visual `rows` from `first` (at most `count`), row 0's top-left at `origin`, in `st` (lines
/// starting with `#` in `heading`, when given); `caret`: draw it on its row (`Some(visible)`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_rows(ui: &mut Ui, ta: &TextArea, rows: &[(usize, usize)], first: usize, count: usize, origin: Point, st: Style, heading: Option<Style>, caret: Option<bool>) {
    let text = ta.text();
    let cur = ta.cursor();
    let crow = cursor_row(rows, cur);
    // whether the row's line is a heading (looked up once, then carried along the rows of each line)
    let mut in_heading = false;
    for (n, &(s, e)) in rows.iter().enumerate().skip(first).take(count) {
        let y = origin.y + n as f32 * ROW_H;
        if heading.is_some() && (n == first || s == 0 || text.as_bytes()[s - 1] == b'\n') {
            let line = text[..s].rfind('\n').map_or(0, |i| i + 1);
            in_heading = text[line..].starts_with('#');
        }
        let st = match heading {
            Some(h) if in_heading => h,
            _ => st,
        };
        let b = ui.line(&text[s..e], st, None);
        ui.draw(&b, Point::new(origin.x, y + (ROW_H - b.h) / 2.0));
        if n == crow && caret == Some(true) {
            let x = b.layout.caret_x(cur - s) / ui.scale;
            beam(ui, Point::new(origin.x + x, y + 2.0));
        }
    }
}

/// The caret's beam at `at` (its top), just after the character before it.
pub(crate) fn beam(ui: &mut Ui, at: Point) {
    ui.top_fill(Rect::new(at.x, at.y, 1.6, ROW_H - 4.0), 0.8, ui.t.accent);
}

/// The byte a click at `p` puts the cursor on: the nearest gap between characters on the row under it.
pub(crate) fn byte_at(ta: &TextArea, g: &Geo, p: Point) -> usize {
    let p = Point::new(p.x, p.y.clamp(g.view.y, (g.view.bottom() - 1.0).max(g.view.y)));
    let rows = ta.rows(g.width);
    let n = (((p.y - g.origin.y) / ROW_H).floor().max(0.0) as usize).min(rows.len().saturating_sub(1));
    let Some(&(s, e)) = rows.get(n) else { return 0 };
    let text = ta.text();
    let mut x = g.origin.x;
    let mut at = e;
    for (i, c) in text[s..e].char_indices() {
        let w = c.width().unwrap_or(0) as f32 * g.adv;
        if p.x < x + w / 2.0 {
            at = s + i;
            break;
        }
        x += w;
    }
    // a soft-wrapped row ends where the next one starts: stay on the clicked row
    if at == e && e > s && rows.get(n + 1).is_some_and(|r| r.0 == e) {
        at = text[..e].char_indices().next_back().map_or(s, |(i, _)| i).max(s);
    }
    at
}

/// Dim the window and float a `w`×`h` card (at most the window less a margin) a third of the way down,
/// lifted while it opens (`k`). `grow`: room a passing line asks for (an error over the buttons); the card
/// opens upward for it while it can (what doesn't fit comes out of the body), so the buttons never move
/// under the pointer. Returns the card.
pub(crate) fn card(ui: &mut Ui, area: Rect, open: &Opened, w: f32, h: f32, grow: f32, k: f32) -> Rect {
    widgets::scrim(ui, area, k, Action::None);
    let h = h.min(area.h - 48.0).max(120.0);
    let top = (area.y + area.h * 0.36 - h / 2.0).min(area.bottom() - 24.0 - h).max(area.y + 24.0);
    // a page that grows or shrinks glides to its new place
    let top = ui.ease(("watchdog-card", open.key()), top, 160.0);
    let up = grow.min(top - area.y - 24.0).max(0.0);
    let up = ui.ease(("watchdog-grow", open.key()), up, 120.0);
    let r = Rect::new((area.x + (area.w - w) / 2.0).round(), top - up + (1.0 - k) * 8.0, w, h + up);
    widgets::card(ui, r, radius::OVERLAY);
    r
}

/// The command band across the bottom `h` px of `card`: a step down from the card with a hairline over it,
/// where the buttons sit (WinUI's dialog). Returns it.
pub(crate) fn band(ui: &mut Ui, card: Rect, h: f32) -> Rect {
    let t = ui.t;
    // inside the card's edge, which stays drawn
    let edge = ui.scale.round().max(1.0) / ui.scale;
    let r = Rect::new(card.x + edge, card.bottom() - h, card.w - 2.0 * edge, h - edge);
    ui.round_corners(r, Corners::bottom(radius::OVERLAY - edge), t.raised.mix(t.bg, if t.light { 0.9 } else { 0.55 }));
    ui.hline(r.x, r.y, r.w, t.hairline);
    Rect { h, ..r }
}

/// The dialog's title row: the watchdog mark, `title` and, after a dot, `detail` (muted).
pub(crate) fn title(ui: &mut Ui, card: Rect, title: &str, detail: Option<&str>) {
    let t = ui.t;
    let x = card.x + PAD;
    let y = card.y + 22.0;
    let right = card.right() - PAD;
    let size = 18.0;
    ui.icon(Rect::new(x - 1.0, y, 20.0, 26.0), ICON, 18.0, t.text);
    let tx = x + 28.0;
    let tb = ui.line(title, Style::new(size, weight::SEMIBOLD, t.text), Some(right - tx));
    ui.draw(&tb, Point::new(tx, y + (26.0 - tb.h) / 2.0));
    if let Some(d) = detail {
        let dx = tx + tb.w;
        if right - dx > 40.0 {
            let db = ui.line(&format!(" · {d}"), Style::new(size, weight::REGULAR, t.muted), Some(right - dx));
            ui.draw(&db, Point::new(dx, y + (26.0 - db.h) / 2.0));
        }
    }
}

/// A group heading (sentence case, semibold, as WinUI's) and, optionally, a quiet note at the right.
pub(crate) fn section(ui: &mut Ui, r: Rect, label: &str, right: &str) {
    let t = ui.t;
    let st = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let rw = if right.is_empty() { 0.0 } else { ui.measure(right, st) + 12.0 };
    widgets::section(ui, Rect { w: r.w - rw, ..r }, label);
    if !right.is_empty() {
        ui.label_in(Rect::new(r.right() - rw + 12.0, r.y, rw - 12.0, r.h), right, st, Align::Right);
    }
}

/// A button with its key in a small chip after the label (`chord` may be empty). `ink` overrides the
/// label's colour; `focused`: the keyboard is on it (an outline around it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn key_button(ui: &mut Ui, r: Rect, label: &str, chord: &str, kind: widgets::Kind, ink: Option<Color>, focused: bool, action: Action) {
    let t = ui.t;
    let hot = ui.hover_amount(("watchdog-button", label, r.x as i32, r.y as i32), r);
    let down = ui.pressing && hot > 0.5;
    // the kit's buttons: 4px corners, a filled accent (or red) for the main one, a control for the others
    let def = match kind {
        widgets::Kind::Primary => {
            ui.round(r, radius::CONTROL, if down { t.accent.fade(0.8) } else { t.accent.fade(1.0 - 0.1 * hot) });
            let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
            ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, Color::rgba(0.0, 0.0, 0.0, 0.18));
            t.on_accent
        }
        widgets::Kind::Danger => {
            ui.round(r, radius::CONTROL, if down { t.danger.fade(0.8) } else { t.danger.fade(1.0 - 0.1 * hot) });
            t.ink_on(t.danger)
        }
        widgets::Kind::Secondary => {
            widgets::control(ui, r, if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) });
            t.text
        }
        widgets::Kind::Ghost => {
            if hot > 0.0 || down {
                ui.round(r, radius::CONTROL, t.text.with_alpha(if down { 0.04 } else { 0.06 * hot }));
            }
            t.text
        }
    };
    let ink = ink.unwrap_or(def);
    let ink = if down { ink.fade(0.8) } else { ink };
    if focused {
        ui.outline(r.outset(3.0), radius::CONTROL + 3.0, 1.0, t.accent);
    }
    let lb = ui.line(label, Style::new(size::BODY, weight::MEDIUM, ink), None);
    let chip = (!chord.is_empty()).then(|| ui.line(chord, Style::new(size::CAPTION, weight::MEDIUM, ink.fade(0.72)), None));
    let chip_w = chip.as_ref().map_or(0.0, |b| b.w + 10.0);
    let gap = if chip.is_some() { 8.0 } else { 0.0 };
    let x = r.x + (r.w - lb.w - gap - chip_w) / 2.0;
    ui.draw(&lb, Point::new(x, r.y + (r.h - lb.h) / 2.0));
    if let Some(cb) = chip {
        let c = Rect::new(x + lb.w + gap, r.y + (r.h - 18.0) / 2.0, chip_w, 18.0);
        let strong = matches!(kind, widgets::Kind::Primary | widgets::Kind::Danger);
        ui.round(c, radius::CONTROL, ink.with_alpha(if strong { 0.16 } else { 0.08 }));
        ui.draw(&cb, Point::new(c.x + 5.0, c.y + (18.0 - cb.h) / 2.0 - 0.5));
    }
    ui.hit(r, action);
}

/// Width a [`key_button`] wants.
pub(crate) fn key_button_width(ui: &mut Ui, label: &str, chord: &str) -> f32 {
    let lw = ui.measure(label, Style::new(size::BODY, weight::MEDIUM, ui.t.text));
    let cw = if chord.is_empty() { 0.0 } else { ui.measure(chord, Style::new(size::CAPTION, weight::MEDIUM, ui.t.text)) + 18.0 };
    (lw + cw + 32.0).max(88.0)
}

/// Width [`widgets::kbd`] takes for `chord` (one keycap per key, 3px apart).
pub(crate) fn kbd_width(ui: &mut Ui, chord: &str) -> f32 {
    let st = Style::new(size::CAPTION, weight::MEDIUM, ui.t.muted);
    let keys = widgets::chord_keys(chord);
    let caps: f32 = keys.iter().map(|k| (ui.measure(k, st) + 10.0).max(18.0)).sum();
    caps + 3.0 * keys.len().saturating_sub(1) as f32
}

/// Key hints (keycaps and their words) flowing over lines inside `w` from `at`. Returns the height they
/// take; drawn only when `draw`.
pub(crate) fn hints(ui: &mut Ui, at: Point, w: f32, pairs: &[(&str, &str)], draw: bool) -> f32 {
    let t = ui.t;
    let st = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let (mut x, mut y) = (at.x, at.y);
    for (key, word) in pairs {
        let kw = kbd_width(ui, key);
        let wb = ui.line(word, st, None);
        let item = kw + 6.0 + wb.w;
        if x > at.x && x + item > at.x + w {
            x = at.x;
            y += 24.0;
        }
        if draw {
            widgets::kbd(ui, Point::new(x, y), key);
            ui.draw(&wb, Point::new(x + kw + 6.0, y + (18.0 - wb.h) / 2.0));
        }
        x += item + 16.0;
    }
    if pairs.is_empty() { 0.0 } else { y - at.y + 18.0 }
}

/// The logo of the agent a label names by its first word ("claude · work · opus" → Claude's).
pub(crate) fn agent_mark(label: &str) -> Option<Mark> {
    let first = label.split(|c: char| c == '·' || c.is_whitespace()).find(|w| !w.is_empty())?;
    Harness::ALL.into_iter().find(|h| h.label().eq_ignore_ascii_case(first)).map(brand::harness)
}

/// `text` in `st`, after its agent's logo when it names one, inside `r` (vertically centred, aligned left or
/// right). Returns the width it took.
fn marked(ui: &mut Ui, r: Rect, text: &str, st: Style, align: Align) -> f32 {
    let mark = agent_mark(text);
    let m = if mark.is_some() { st.size + 6.0 } else { 0.0 };
    let b = ui.line(text, st, Some((r.w - m).max(10.0)));
    let x = match align {
        Align::Right => r.right() - b.w - m,
        _ => r.x,
    };
    if let Some(mk) = mark {
        ui.mark(mk, Rect::new(x, r.y, st.size, r.h), st.size, None);
    }
    ui.draw(&b, Point::new(x + m, r.y + (r.h - b.h) / 2.0));
    b.w + m
}

/// Keep `top..top + h` (content px) inside a view of `view` px scrolled to `scroll`.
fn reveal(scroll: &mut f32, top: f32, h: f32, view: f32, margin: f32) {
    if top - margin < *scroll {
        *scroll = top - margin;
    } else if top + h + margin > *scroll + view {
        *scroll = top + h + margin - view;
    }
}

// ---------------------------------------------------------------------------------------------- the card

/// What the card remembers between frames (layout only: everything else is the view's own state).
#[derive(Default)]
struct Memo {
    open: Opened,
    /// the page drawn last (true: the status page); the body scrolls back up when it changes
    page: Option<bool>,
    /// the body's scroll, px, and how far it can go
    scroll: f32,
    max_scroll: f32,
    /// the focus with its cursor and text length, last frame: the body scrolls to follow a change
    focus: Option<(Field, usize, usize)>,
    /// the focused milestone's rows when it outgrows its field: (milestone, first row shown)
    window: (usize, usize),
    /// where each milestone's text sits
    fields: Vec<Option<Geo>>,
    /// the choices shown for Wake after and Budget (an off-list value joins them)
    wake: Vec<u64>,
    budget: Vec<u32>,
    caret: Caret,
}

thread_local! {
    static MEMO: RefCell<Memo> = RefCell::new(Memo::default());
}

/// Draw the watchdog card over the window (`area`: the whole window).
pub(crate) fn draw(s: &mut WatchdogView, ui: &mut Ui, area: Rect, dog: Option<&WatchdogSnapshot>, cx: &Cx) -> bool {
    if area.w < 420.0 || area.h < 300.0 {
        return false;
    }
    let mut m = MEMO.with(|c| c.take());
    // the terminal UI's hit list is unused here: a mark in it tells this view from a new one at its address
    let new = s.hits.is_empty();
    if new {
        s.hits.push((ratatui::layout::Rect::default(), super::Hit::Field(Field::Cancel)));
    }
    if m.open.frame(ui, &*s as *const WatchdogView as usize, new) {
        m = Memo { open: m.open, ..Memo::default() };
    }
    let status = !s.editing && dog.is_some();
    if m.page != Some(status) {
        m.page = Some(status);
        m.scroll = 0.0;
        m.focus = None;
    }
    let k = m.open.amount(ui);
    match dog {
        Some(d) if status => status_page(s, ui, area, d, &mut m, k),
        _ => form(s, ui, area, dog, &mut m, k, cx.focused),
    }
    MEMO.with(|c| c.replace(m));
    true
}

/// Target `n` was clicked: what the terminal UI's click on it does, through the view's own methods. A window
/// adds a little: buttons act on the first click, a choice or a switch sets its value at once, the caret
/// goes where a milestone was clicked, and a milestone's × removes it (ctrl+d).
pub(crate) fn click(s: &mut WatchdogView, n: usize, p: Point, _mods: KeyModifiers, _count: u32, dog: Option<&WatchdogSnapshot>) -> Out {
    match n {
        // the status page's buttons press at once (stop arms on the first press)
        _ if (BUTTON..BUTTON + BUTTONS.len()).contains(&n) => s.press(BUTTONS[n - BUTTON], dog),
        // in the terminal the first click only focuses these
        PRIMARY | CANCEL => {
            let f = if n == PRIMARY { Field::Primary } else { Field::Cancel };
            s.focus = f;
            s.change(f, 0, dog)
        }
        // an option row: focus it; clicked again, change it (as enter would)
        _ if (ROW..ROW + OPTS.len()).contains(&n) => {
            let f = OPTS[n - ROW];
            if s.focus == f {
                s.change(f, 0, dog)
            } else {
                s.set_focus(f, false);
                Out::None
            }
        }
        _ if (SWITCH..SWITCH + OPTS.len()).contains(&n) => {
            let f = OPTS[n - SWITCH];
            s.set_focus(f, false);
            s.change(f, 0, dog)
        }
        _ if (WAKE..WAKE + 50).contains(&n) => {
            let Some(want) = MEMO.with(|c| c.borrow().wake.get(n - WAKE).copied()) else { return Out::None };
            s.set_focus(Field::Wake, false);
            for _ in 0..16 {
                if s.idle_secs == want {
                    break;
                }
                s.change(Field::Wake, if want > s.idle_secs { 1 } else { -1 }, dog);
            }
            Out::None
        }
        _ if (BUDGET..BUDGET + 50).contains(&n) => {
            let Some(want) = MEMO.with(|c| c.borrow().budget.get(n - BUDGET).copied()) else { return Out::None };
            s.set_focus(Field::Budget, false);
            for _ in 0..16 {
                if s.max_wakes == want {
                    break;
                }
                s.change(Field::Budget, if want > s.max_wakes { 1 } else { -1 }, dog);
            }
            Out::None
        }
        _ if (MILESTONE..REMOVE).contains(&n) => {
            let i = n - MILESTONE;
            if i >= s.milestones.len() {
                return Out::None;
            }
            s.set_focus(Field::Milestone(i), false);
            // the caret goes where you clicked
            if let Some(g) = MEMO.with(|c| c.borrow().fields.get(i).copied().flatten()) {
                let at = byte_at(&s.milestones[i], &g, p);
                s.milestones[i].set_cursor(at);
            }
            Out::None
        }
        _ if (REMOVE..REMOVE + 1000).contains(&n) => {
            let i = n - REMOVE;
            if i < s.milestones.len() {
                s.remove(i);
            }
            Out::None
        }
        _ => Out::None,
    }
}

/// The wheel over the card scrolls its body.
pub(crate) fn wheel(_s: &mut WatchdogView, _n: Option<usize>, down: bool) {
    MEMO.with(|c| {
        let mut m = c.borrow_mut();
        m.scroll = (m.scroll + if down { 56.0 } else { -56.0 }).clamp(0.0, m.max_scroll);
    });
}

/// The body's viewport: clip to it and return the eased scroll to draw at.
fn body(ui: &mut Ui, m: &mut Memo, view: Rect, content: f32) -> f32 {
    m.max_scroll = (content - view.h).max(0.0);
    m.scroll = m.scroll.clamp(0.0, m.max_scroll);
    let off = ui.ease(("watchdog-scroll", m.open.key(), m.page), m.scroll, 90.0);
    ui.clip(view, 0.0);
    off
}

/// Close the body: hairlines where content runs under the title or the footer, and the scrollbar.
fn end_body(ui: &mut Ui, m: &Memo, card: Rect, view: Rect, content: f32, off: f32) {
    ui.unclip();
    let t = ui.t;
    if off > 0.5 {
        ui.hline(card.x, view.y, card.w, t.hairline);
    }
    if off < m.max_scroll - 0.5 {
        ui.hline(card.x, view.bottom(), card.w, t.hairline);
    }
    let hot = if ui.hovered(view) { 1.0 } else { 0.55 };
    widgets::scrollbar(ui, Rect::new(card.x, view.y, card.w - 4.0, view.h), content, view.h, off, hot);
}

// ---------------------------------------------------------------------------------------------- the form

/// A piece of the form, top to bottom.
enum Block {
    Intro(TextBox),
    Watches,
    Section(&'static str, String),
    Milestone(usize),
    Opt(usize),
    Reviewer,
    Gap,
}

/// The form's opening words. Wake-ups go to the orchestrator now, so a new plan's says so (the terminal UI's
/// still describes a fresh reviewer on the roomiest login: DD-012 rewrites it); editing keeps its words.
fn form_intro(s: &WatchdogView, existing: bool) -> String {
    if existing {
        return super::intro(s, true);
    }
    let name = &s.name;
    format!("When {name} finishes a turn and goes quiet, bro wakes the orchestrator, starting one if none is running. It reads the chat and the code, checks the work against these milestones and hands {name} its next task.")
}

/// The Reviewer row's help (the terminal UI's "picked fresh each wake-up" is the old model, DD-012).
const REVIEWER_HELP: &str = "every wake-up goes to it · bro starts one if none is running";

fn opt_copy(s: &WatchdogView, f: Field) -> (&'static str, String) {
    match f {
        Field::Wake => ("Wake after", "after each stop · a question waits 2 min or more".into()),
        Field::Fresh => ("Fresh context", if s.fresh { "may clear the conversation before a new milestone" } else { "keeps one conversation going" }.into()),
        Field::Second => ("Second opinions", if s.second { "may open a helper, say an Opus pass on Codex UI work".into() } else { format!("only ever talks to {}", s.name) }),
        _ => ("Budget", "then it stands down and tells you".into()),
    }
}

/// The choices a stepper offers, with an off-list value (a hand-edited setting) in its place among them.
fn choices<T: Copy + PartialOrd>(list: &[T], value: T) -> (Vec<T>, usize) {
    let mut v = list.to_vec();
    if !v.contains(&value) {
        let at = v.iter().position(|c| *c > value).unwrap_or(v.len());
        v.insert(at, value);
    }
    let sel = v.iter().position(|c| *c == value).unwrap_or(0);
    (v, sel)
}

fn form(s: &mut WatchdogView, ui: &mut Ui, area: Rect, dog: Option<&WatchdogSnapshot>, m: &mut Memo, k: f32, window_focused: bool) {
    let t = ui.t;
    let existing = dog.is_some();
    let w = WIDTH.min(area.w - 48.0);
    let cw = w - 2.0 * PAD;
    // the milestones' text width in terminal cells: the rows the keys move through are the rows drawn
    let gutter = if existing { 52.0 } else { 30.0 };
    let field_w = cw - gutter;
    let adv = mono_advance(ui, MONO);
    s.field_width = ((field_w - 2.0 * FIELD_X - REMOVE_W) / adv).floor().max(4.0) as usize;

    // the blocks and their heights
    let mut blocks: Vec<(f32, Block)> = vec![];
    let intro = ui.para(&form_intro(s, existing), Style::new(size::BODY, weight::REGULAR, t.muted), cw, Some(12));
    blocks.push((intro.h + 8.0, Block::Intro(intro)));
    blocks.push((40.0, Block::Watches));
    let count = s.milestones.iter().filter(|m| !m.is_blank()).count();
    blocks.push((32.0, Block::Section("Milestones", format!("{count} · in order · the first one starts now"))));
    for i in 0..s.milestones.len() {
        let rows = s.milestones[i].height(s.field_width).clamp(1, super::FIELD_ROWS);
        blocks.push((rows as f32 * ROW_H + 2.0 * FIELD_Y + 8.0, Block::Milestone(i)));
    }
    blocks.push((16.0, Block::Gap));
    blocks.push((30.0, Block::Section("How it works", String::new())));
    let stack = cw < STACK_BELOW;
    for (i, f) in OPTS.iter().enumerate() {
        let tall = stack && matches!(f, Field::Wake | Field::Budget);
        blocks.push((if tall { OPT_H + STACK_H } else { OPT_H }, Block::Opt(i)));
    }
    blocks.push((OPT_H, Block::Reviewer));
    let content = blocks.iter().map(|b| b.0).sum::<f32>() + 4.0;

    // the command band: the hints beside the buttons
    let primary = if existing { "Save plan" } else { "Start watchdog" };
    let pw = key_button_width(ui, primary, "ctrl+s");
    let cnw = key_button_width(ui, "Cancel", "esc");
    let pairs: Vec<(&str, &str)> = s.hints().into_iter().filter(|(key, _)| *key != "ctrl+s" && *key != "esc").collect();
    let hints_w = cw - pw - cnw - 10.0 - 24.0;
    let mut hints_h = hints(ui, Point::new(0.0, 0.0), hints_w, &pairs, false);
    // more than two lines of hints don't help: the buttons say the main keys
    let pairs = if hints_h > 48.0 { vec![] } else { pairs };
    if pairs.is_empty() {
        hints_h = 0.0;
    }
    let foot_h = 2.0 * BAND_PAD + hints_h.max(BUTTON_H);
    // the error over the band
    let error = s.error.clone().map(|e| ui.para(&e, Style::new(size::BODY, weight::MEDIUM, t.danger), cw - 26.0, Some(3)));
    let line_h = error.as_ref().map_or(0.0, |b| b.h.max(18.0) + 16.0);

    let r = card(ui, area, &m.open, w, HEAD_H + content + foot_h, line_h, k);
    title(ui, r, "watchdog", Some(s.title.as_str()));
    let view = Rect::new(r.x, r.y + HEAD_H, r.w, r.h - HEAD_H - foot_h - line_h);
    let x0 = r.x + PAD;

    // the focused milestone's rows (it scrolls inside its field once it outgrows six)
    for i in 0..s.milestones.len() {
        let focused = s.focus == Field::Milestone(i);
        window(m, i, &s.milestones[i], s.field_width, focused);
    }
    // a new focus (or the caret moving) scrolls the body to it
    let sig = match s.focus {
        Field::Milestone(i) => s.milestones.get(i).map_or((s.focus, 0, 0), |ta| (s.focus, ta.cursor(), ta.text().len())),
        f => (f, 0, 0),
    };
    if m.focus != Some(sig) {
        m.focus = Some(sig);
        m.scroll = follow(s, &blocks, m.window, m.scroll, view.h);
    }

    let off = body(ui, m, view, content);
    m.fields = vec![None; s.milestones.len()];
    let caret_on = match s.focus {
        Field::Milestone(i) if window_focused => s.milestones.get(i).map(|ta| (ta.cursor(), ta.text().len())),
        _ => None,
    };
    let caret_on = caret_on.map(|(c, l)| m.caret.on(ui, c, l));
    let mut y = view.y - off;
    for (h, b) in &blocks {
        let top = y;
        y += h;
        if y < view.y - 4.0 || top > view.bottom() + 4.0 {
            continue;
        }
        match b {
            Block::Gap => {}
            Block::Intro(b) => ui.draw(b, Point::new(x0, top)),
            Block::Watches => {
                // who it watches: their logo and name
                let st = Style::new(size::SMALL, weight::REGULAR, t.muted);
                let lb = ui.line("watches", st, None);
                ui.draw(&lb, Point::new(x0, top + 1.0));
                marked(ui, Rect::new(x0 + lb.w + 8.0, top + 1.0, cw - lb.w - 8.0, lb.h), &s.agent, Style::new(size::SMALL, weight::MEDIUM, t.text), Align::Left);
            }
            Block::Section(label, right) => section(ui, Rect::new(x0, top + 4.0, cw, 18.0), label, right),
            Block::Milestone(i) => milestone(s, ui, m, *i, Rect::new(x0, top, cw, h - 8.0), gutter, adv, existing, caret_on),
            Block::Opt(i) => option(s, ui, m, *i, Rect::new(x0, top, cw, *h), stack),
            Block::Reviewer => reviewer(ui, Rect::new(x0, top, cw, *h)),
        }
    }
    end_body(ui, m, r, view, content, off);

    // the error over the band, then the band: the hints at the left, cancel and the main button at the right
    if let Some(e) = &error {
        let ey = view.bottom() + 8.0;
        ui.icon(Rect::new(x0 - 1.0, ey - 1.0, 18.0, 20.0), ALERT, 14.0, t.danger);
        ui.draw(e, Point::new(x0 + 24.0, ey));
    }
    let foot = band(ui, r, foot_h);
    let by = foot.y + (foot.h - BUTTON_H) / 2.0;
    let pr = Rect::new(r.right() - PAD - pw, by, pw, BUTTON_H);
    let cr = Rect::new(pr.x - 8.0 - cnw, by, cnw, BUTTON_H);
    key_button(ui, cr, "Cancel", "esc", widgets::Kind::Secondary, None, s.focus == Field::Cancel, Action::Overlay(CANCEL));
    key_button(ui, pr, primary, "ctrl+s", widgets::Kind::Primary, None, s.focus == Field::Primary, Action::Overlay(PRIMARY));
    let hy = foot.y + (foot.h - hints_h.max(18.0)) / 2.0;
    hints(ui, Point::new(x0, hy), hints_w, &pairs, true);
}

/// The body's scroll (content px) once the focus moved: the focused milestone's caret row or the focused
/// option kept in view; on the buttons, the end of the form, as the terminal UI scrolls there (`f32::MAX`,
/// which `body` clamps). `window`: the focused milestone's first row shown.
fn follow(s: &WatchdogView, blocks: &[(f32, Block)], window: (usize, usize), scroll: f32, view: f32) -> f32 {
    if matches!(s.focus, Field::Primary | Field::Cancel) {
        return f32::MAX;
    }
    let mut scroll = scroll;
    let mut y = 0.0;
    for (h, b) in blocks {
        match b {
            Block::Milestone(i) if s.focus == Field::Milestone(*i) => {
                let ta = &s.milestones[*i];
                let rows = ta.rows(s.field_width);
                let first = if window.0 == *i { window.1 } else { 0 };
                let crow = cursor_row(&rows, ta.cursor()).saturating_sub(first) as f32;
                reveal(&mut scroll, y + FIELD_Y + crow * ROW_H, ROW_H, view, FIELD_Y + 12.0);
            }
            Block::Opt(i) if s.focus == OPTS[*i] => reveal(&mut scroll, y, *h, view, 8.0),
            _ => {}
        }
        y += h;
    }
    scroll
}

/// The first row shown of milestone `i` (its field shows at most `FIELD_ROWS`): the focused one keeps its
/// caret in view, the others show their start.
fn window(m: &mut Memo, i: usize, ta: &TextArea, width: usize, focused: bool) -> usize {
    let rows = ta.rows(width);
    let room = super::FIELD_ROWS;
    if rows.len() <= room || !focused {
        return 0;
    }
    let crow = cursor_row(&rows, ta.cursor());
    let mut first = if m.window.0 == i { m.window.1 } else { 0 };
    if crow < first {
        first = crow;
    } else if crow >= first + room {
        first = crow + 1 - room;
    }
    first = first.min(rows.len() - room);
    m.window = (i, first);
    first
}

/// A milestone: its number, its status (plans already running) and its field.
#[allow(clippy::too_many_arguments)]
fn milestone(s: &WatchdogView, ui: &mut Ui, m: &mut Memo, i: usize, r: Rect, gutter: f32, adv: f32, existing: bool, caret: Option<bool>) {
    let t = ui.t;
    let ta = &s.milestones[i];
    let on = s.focus == Field::Milestone(i);
    let status = s.kept.get(i).map_or(MilestoneStatus::Pending, |k| k.0);
    let mid = r.y + FIELD_Y + ROW_H / 2.0;
    let num = if on { Style::new(size::SMALL, weight::SEMIBOLD, t.text) } else { Style::new(size::SMALL, weight::MEDIUM, t.muted) };
    ui.label_in(Rect::new(r.x, mid - 9.0, 16.0, 18.0), &(i + 1).to_string(), num, Align::Right);
    if existing {
        status_mark(ui, Point::new(r.x + 34.0, mid), status);
    }
    // a WinUI text box: a control at rest (its bottom edge a touch stronger), the accent all round with the keys
    let fr = Rect::new(r.x + gutter, r.y, r.w - gutter, r.h);
    let hot = ui.hover_amount(("watchdog-milestone", i), fr);
    if on {
        ui.panel(fr, radius::CONTROL, t.field_focus, t.accent.with_alpha(0.8));
    } else {
        ui.panel(fr, radius::CONTROL, t.control.mix(t.control_hover, hot), t.stroke);
        let inner = Rect::new(fr.x + radius::CONTROL, fr.y, (fr.w - 2.0 * radius::CONTROL).max(0.0), fr.h);
        ui.hline(inner.x, fr.bottom() - ui.hair(), inner.w, t.muted.with_alpha(0.45));
    }
    let rows = ta.rows(s.field_width);
    let first = if on && m.window.0 == i { m.window.1 } else { 0 };
    let shown = rows.len().clamp(1, super::FIELD_ROWS);
    let origin = Point::new(fr.x + FIELD_X, fr.y + FIELD_Y - first as f32 * ROW_H);
    let view = Rect::new(fr.x, fr.y + FIELD_Y, fr.w, shown as f32 * ROW_H);
    m.fields[i] = Some(Geo { origin, width: s.field_width, adv, view });
    ui.clip(fr.inset(1.0), radius::CONTROL - 1.0);
    if ta.text().is_empty() {
        let placeholder = if i == 0 { "the first milestone and what done looks like, e.g. offline sync: writes queue, retry with backoff, tests cover it" } else { "the next milestone · enter on an empty one finishes the list" };
        let st = Style { italic: true, ..Style::new(size::SMALL, weight::REGULAR, t.faint) };
        let room = fr.w - 2.0 * FIELD_X - if s.milestones.len() > 1 { REMOVE_W } else { 0.0 };
        let pb = ui.line(placeholder, st, Some(room));
        ui.draw(&pb, Point::new(origin.x + 4.0, fr.y + FIELD_Y + (ROW_H - pb.h) / 2.0));
        if on && caret == Some(true) {
            beam(ui, Point::new(origin.x, fr.y + FIELD_Y + 2.0));
        }
    } else {
        let ink = if status == MilestoneStatus::Done { t.muted } else { t.text };
        draw_rows(ui, ta, &rows, first, shown, origin, Style::mono(MONO, ink), None, if on { caret } else { None });
    }
    ui.unclip();
    ui.hit_with(fr, Action::Overlay(MILESTONE + i), Cursor::Text);
    // ctrl+d's button, on hover
    if hot > 0.02 && (s.milestones.len() > 1 || !ta.text().is_empty()) {
        let xr = Rect::new(fr.right() - 6.0 - 22.0, fr.y + FIELD_Y + ROW_H / 2.0 - 11.0, 22.0, 22.0);
        widgets::icon_button(ui, xr, CLOSE, t.muted.fade(hot), Action::Overlay(REMOVE + i));
    }
}

/// A milestone's status, as an icon on its own (the terminal UI's ✓ ▸ ○): a check when done, play on the
/// active one, a ring while it waits.
fn status_mark(ui: &mut Ui, c: Point, st: MilestoneStatus) {
    let t = ui.t;
    let r = Rect::new(c.x - 9.0, c.y - 9.0, 18.0, 18.0);
    match st {
        MilestoneStatus::Done => ui.icon(r, CHECK_CIRCLE, 16.0, t.good),
        MilestoneStatus::Active => ui.icon(r, PLAY_CIRCLE, 16.0, t.accent),
        MilestoneStatus::Pending => ui.icon(r, CIRCLE, 16.0, t.faint),
    }
}

/// A label with its help line under it, centred in `r`.
fn label_help(ui: &mut Ui, r: Rect, label: &str, help: &str, strong: bool) {
    let t = ui.t;
    let lb = ui.line(label, Style::new(size::TITLE, if strong { weight::MEDIUM } else { weight::REGULAR }, t.text), Some(r.w.max(10.0)));
    let hb = ui.line(help, Style::new(size::SMALL, weight::REGULAR, t.muted), Some(r.w.max(10.0)));
    let y = r.y + (r.h - lb.h - 1.0 - hb.h) / 2.0;
    ui.draw(&lb, Point::new(r.x, y));
    ui.draw(&hb, Point::new(r.x, y + lb.h + 1.0));
}

/// Who reviews: the orchestrator, which every wake-up goes to now (the terminal UI's row still names the login
/// a fresh reviewer would run on, DD-012), with its star at the right.
fn reviewer(ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let st = Style::new(size::BODY, weight::MEDIUM, t.text);
    let name = "the orchestrator";
    let nw = ui.measure(name, st);
    let w = nw + 22.0;
    label_help(ui, Rect::new(r.x, r.y, r.w - w - 20.0, r.h), "Reviewer", REVIEWER_HELP, false);
    let mid = r.y + r.h / 2.0;
    ui.icon(Rect::new(r.right() - w, mid - 9.0, 16.0, 18.0), STAR, 15.0, t.gold);
    ui.label_in(Rect::new(r.right() - nw, mid - 10.0, nw, 20.0), name, st, Align::Right);
}

/// An option row: label and help at the left, its control at the right (under them when `stack`, for
/// choices). Focus is an outline around the row.
fn option(s: &WatchdogView, ui: &mut Ui, m: &mut Memo, i: usize, r: Rect, stack: bool) {
    let t = ui.t;
    let f = OPTS[i];
    let on = s.focus == f;
    let stack = stack && matches!(f, Field::Wake | Field::Budget);
    // the row's fill reaches into the card's padding so its text lines up with everything else
    let row = Rect::new(r.x - 12.0, r.y + 3.0, r.w + 24.0, r.h - 6.0);
    let hot = ui.hover_amount(("watchdog-option", i), row);
    if on {
        ui.round(row, radius::CONTROL, t.hover.over(t.raised));
        ui.outline(row, radius::CONTROL, 1.0, t.accent.with_alpha(0.8));
    } else if hot > 0.0 {
        ui.round(row, radius::CONTROL, t.hover.fade(hot));
    }
    ui.hit(row, Action::Overlay(ROW + i));
    // stacked: the text keeps the row's top, the choices sit under it
    let text_h = if stack { OPT_H - 6.0 } else { row.h };
    let mid = if stack { row.y + text_h + 16.0 } else { row.y + row.h / 2.0 };
    let right = r.right();
    let control_w = match f {
        Field::Wake | Field::Budget => {
            let (labels, sel, base, unit): (Vec<String>, usize, usize, &str) = if f == Field::Wake {
                let (v, sel) = choices(&super::super::IDLE_CHOICES, s.idle_secs);
                let labels = v.iter().map(|x| super::super::human(*x)).collect();
                m.wake = v;
                (labels, sel, WAKE, "")
            } else {
                let (v, sel) = choices(&super::super::BUDGET_CHOICES, s.max_wakes);
                let labels = v.iter().map(|x| x.to_string()).collect();
                m.budget = v;
                (labels, sel, BUDGET, "wake-ups")
            };
            let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
            let each = labels.iter().map(|l| ui.measure(l, st)).fold(0.0f32, f32::max) + 20.0;
            let ust = Style::new(size::SMALL, weight::REGULAR, t.muted);
            let uw = if unit.is_empty() { 0.0 } else { ui.measure(unit, ust) + 10.0 };
            let sw = each * labels.len() as f32 + 4.0;
            let sr = if stack { Rect::new(r.x, mid - CTRL_H / 2.0, sw, CTRL_H) } else { Rect::new(right - uw - sw, mid - CTRL_H / 2.0, sw, CTRL_H) };
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            widgets::segmented(ui, sr, &refs, sel, |j| Action::Overlay(base + j));
            if !unit.is_empty() {
                ui.label_in(Rect::new(sr.right() + 10.0, mid - 9.0, uw - 10.0, 18.0), unit, ust, Align::Left);
            }
            if stack { 0.0 } else { sw + uw }
        }
        _ => {
            // WinUI's toggle: what it's at, then the switch, against the right edge
            let value = if f == Field::Fresh { s.fresh } else { s.second };
            let at = Point::new(right - widgets::SWITCH_W, mid - widgets::SWITCH_H / 2.0);
            let st = Style::new(size::BODY, weight::REGULAR, t.text);
            let ww = ui.measure("On", st).max(ui.measure("Off", st));
            let words = Rect::new(at.x - 12.0 - ww, mid - 10.0, ww, 20.0);
            ui.label_in(words, if value { "On" } else { "Off" }, st, Align::Right);
            ui.hit(words, Action::Overlay(SWITCH + i));
            widgets::switch(ui, at, value, Action::Overlay(SWITCH + i));
            widgets::SWITCH_W + 12.0 + ww
        }
    };
    let (label, help) = opt_copy(s, f);
    let room = if control_w > 0.0 { r.w - control_w - 24.0 } else { r.w };
    label_help(ui, Rect::new(r.x, row.y, room, text_h), label, &help, on);
}

// ---------------------------------------------------------------------------------------------- status page

/// A piece of the status page, top to bottom.
enum Part {
    State(TextBox),
    Gap,
    Section(&'static str, String),
    Milestone(usize, TextBox, Option<TextBox>),
    Starting,
    Step(usize),
    Empty(TextBox),
    Run(usize, TextBox),
}

fn status_page(s: &mut WatchdogView, ui: &mut Ui, area: Rect, d: &WatchdogSnapshot, m: &mut Memo, k: f32) {
    let t = ui.t;
    let w = WIDTH.min(area.w - 48.0);
    let cw = w - 2.0 * PAD;
    let name = s.name.clone();
    // the second line under the state
    let note = match (&d.running, &d.finished, d.paused) {
        (Some(r), _, _) => format!("woke because {}", r.reason),
        (None, Some(_), _) => "e adds milestones or wake-ups and it watches again".to_string(),
        (None, None, true) => "p resumes watching".to_string(),
        (None, None, false) => format!(
            "wakes when {name} stops and sits idle {} · fresh context {} · second opinions {}",
            super::super::human(d.plan.idle_secs),
            if d.plan.fresh_context { "on" } else { "off" },
            if d.plan.second_opinions { "on" } else { "off" }
        ),
    };
    let mut parts: Vec<(f32, Part)> = vec![];
    let nb = ui.para(&note, Style::new(size::BODY, weight::REGULAR, t.muted), cw, Some(3));
    parts.push((32.0 + nb.h + 4.0, Part::State(nb)));
    parts.push((20.0, Part::Gap));
    let (done, total) = d.plan.progress();
    parts.push((30.0, Part::Section("Milestones", format!("{done} of {total} done"))));
    let tw = cw - 48.0;
    for (i, ms) in d.plan.milestones.iter().enumerate() {
        let st = match ms.status {
            MilestoneStatus::Done => Style::new(size::BODY, weight::REGULAR, t.muted),
            MilestoneStatus::Active => Style::new(size::BODY, weight::SEMIBOLD, t.text),
            MilestoneStatus::Pending => Style::new(size::BODY, weight::REGULAR, t.text),
        };
        let tb = ui.para(&ms.title, st, tw, Some(3));
        let nb = ms.note.as_ref().map(|n| ui.line(&format!("↳ {n}"), Style::new(size::SMALL, weight::REGULAR, t.muted), Some(tw)));
        let h = tb.h.max(20.0) + nb.as_ref().map_or(0.0, |b| b.h + 2.0) + 8.0;
        parts.push((h, Part::Milestone(i, tb, nb)));
    }
    if let Some(r) = &d.running {
        parts.push((16.0, Part::Gap));
        parts.push((30.0, Part::Section("Right now", String::new())));
        if r.steps.is_empty() {
            parts.push((24.0, Part::Starting));
        }
        for n in r.steps.len().saturating_sub(6)..r.steps.len() {
            parts.push((24.0, Part::Step(n)));
        }
    }
    parts.push((16.0, Part::Gap));
    parts.push((30.0, Part::Section("Last wake-ups", String::new())));
    if d.runs.is_empty() && d.running.is_none() {
        let eb = ui.para(&format!("None yet. The first comes once {name} stops and sits idle for {}.", super::super::human(d.plan.idle_secs)), Style::new(size::BODY, weight::REGULAR, t.muted), cw, Some(6));
        parts.push((eb.h + 4.0, Part::Empty(eb)));
    }
    for (n, run) in d.runs.iter().enumerate().rev() {
        let summary = run.summary.split_whitespace().collect::<Vec<_>>().join(" ");
        let sb = ui.para(&summary, Style::new(size::BODY, weight::REGULAR, if run.ok { t.text } else { t.danger }), cw - 84.0, Some(3));
        parts.push((22.0 + sb.h + 12.0, Part::Run(n, sb)));
    }
    let content = parts.iter().map(|p| p.0).sum::<f32>() + 4.0;

    // the command band: the armed stop over it
    let labels: Vec<(&str, &str, widgets::Kind)> = BUTTONS
        .iter()
        .map(|b| match b {
            Button::Wake => ("Wake now", "w", widgets::Kind::Primary),
            Button::Pause if d.paused => ("Resume", "p", widgets::Kind::Secondary),
            Button::Pause => ("Pause", "p", widgets::Kind::Secondary),
            Button::Edit => ("Edit plan", "e", widgets::Kind::Secondary),
            Button::Stop if s.stop_armed => ("Stop watchdog", "s", widgets::Kind::Danger),
            Button::Stop => ("Stop watchdog", "s s", widgets::Kind::Ghost),
            Button::Close => ("Close", "esc", widgets::Kind::Secondary),
        })
        .collect();
    let mut widths: Vec<f32> = labels.iter().map(|(l, c, _)| key_button_width(ui, l, c)).collect();
    let fits = |w: &[f32]| w.iter().sum::<f32>() + 8.0 * 3.0 + 20.0 <= cw;
    // no room for the keys: plain buttons, then snug ones
    let chips = fits(&widths);
    if !chips {
        widths = labels.iter().map(|(l, _, _)| key_button_width(ui, l, "")).collect();
        if !fits(&widths) {
            widths = labels.iter().map(|(l, _, _)| ui.measure(l, Style::new(size::BODY, weight::MEDIUM, t.text)) + 22.0).collect();
        }
    }
    let foot_h = 2.0 * BAND_PAD + BUTTON_H;
    // the armed stop's warning over the band (the buttons stay put for the second press)
    let line_h = if s.stop_armed { 32.0 } else { 0.0 };

    let r = card(ui, area, &m.open, w, HEAD_H + content + foot_h, line_h, k);
    let x0 = r.x + PAD;
    title(ui, r, "watchdog", Some(s.title.as_str()));
    let view = Rect::new(r.x, r.y + HEAD_H, r.w, r.h - HEAD_H - foot_h - line_h);
    let off = body(ui, m, view, content);
    let mut y = view.y - off;
    for (h, p) in &parts {
        let top = y;
        y += h;
        if y < view.y - 4.0 || top > view.bottom() + 4.0 {
            continue;
        }
        match p {
            Part::Gap => {}
            Part::State(nb) => state(s, ui, d, Rect::new(x0, top, cw, 26.0), nb),
            Part::Section(label, right) => section(ui, Rect::new(x0, top + 4.0, cw, 18.0), label, right),
            Part::Milestone(i, tb, nb) => {
                let ms = &d.plan.milestones[*i];
                let line = tb.h / tb.layout.lines.len().max(1) as f32;
                let mid = top + line / 2.0;
                status_mark(ui, Point::new(x0 + 8.0, mid), ms.status);
                ui.label_in(Rect::new(x0 + 20.0, mid - 9.0, 18.0, 18.0), &(i + 1).to_string(), Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Right);
                ui.draw(tb, Point::new(x0 + 48.0, top));
                if let Some(nb) = nb {
                    ui.draw(nb, Point::new(x0 + 48.0, top + tb.h + 2.0));
                }
            }
            Part::Starting => {
                ui.spinner(Point::new(x0 + 8.0, top + 11.0), 4.6, 1.6, t.accent);
                ui.label_in(Rect::new(x0 + 24.0, top + 2.0, cw - 24.0, 18.0), "starting the reviewer…", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
            }
            Part::Step(n) => {
                let Some(run) = &d.running else { continue };
                let last = n + 1 == run.steps.len();
                let c = Point::new(x0 + 8.0, top + 11.0);
                if last {
                    ui.spinner(c, 4.6, 1.6, t.accent);
                } else {
                    // a step it has taken
                    ui.icon(Rect::new(c.x - 7.0, c.y - 8.0, 14.0, 16.0), CHECK, 12.0, t.faint);
                }
                let st = Style::new(size::BODY, weight::REGULAR, if last { t.text } else { t.muted });
                ui.label_in(Rect::new(x0 + 24.0, top + 2.0, cw - 24.0, 18.0), &run.steps[*n], st, Align::Left);
            }
            Part::Empty(eb) => ui.draw(eb, Point::new(x0, top)),
            Part::Run(n, sb) => {
                let run = &d.runs[*n];
                let row = Rect::new(x0, top, cw, 20.0);
                ui.label_in(Rect::new(x0, top, 58.0, 20.0), &super::clock(&run.started_at), Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Right);
                let (glyph, c) = if run.ok { (CHECK, t.good) } else { (CLOSE, t.danger) };
                ui.icon(Rect::new(x0 + 64.0, top, 16.0, 20.0), glyph, 13.0, c);
                let steps = format!("{} steps", run.steps.len());
                let sst = Style::new(size::SMALL, weight::REGULAR, t.muted);
                let stw = ui.measure(&steps, sst);
                ui.label_in(Rect::new(row.right() - stw, top, stw, 20.0), &steps, sst, Align::Right);
                marked(ui, Rect::new(x0 + 84.0, top, cw - 84.0 - stw - 16.0, 20.0), &run.reviewer, Style::new(size::SMALL, weight::MEDIUM, t.text), Align::Left);
                ui.draw(sb, Point::new(x0 + 84.0, top + 22.0));
            }
        }
    }
    end_body(ui, m, r, view, content, off);

    // the armed stop's warning, then the band: wake, pause and edit from the left; stop and close at the right
    if s.stop_armed {
        let ay = view.bottom() + 7.0;
        ui.icon(Rect::new(x0 - 1.0, ay, 18.0, 20.0), ALERT, 14.0, t.danger);
        ui.label_in(Rect::new(x0 + 24.0, ay + 1.0, cw - 24.0, 18.0), "press s again to stop the watchdog; the session keeps running", Style::new(size::BODY, weight::MEDIUM, t.danger), Align::Left);
    }
    let foot = band(ui, r, foot_h);
    let by = foot.y + (foot.h - BUTTON_H) / 2.0;
    let mut x = x0;
    for (j, ((label, chord, kind), bw)) in labels.iter().zip(&widths).enumerate().take(3) {
        let br = Rect::new(x, by, *bw, BUTTON_H);
        key_button(ui, br, label, if chips { chord } else { "" }, *kind, None, s.button == BUTTONS[j], Action::Overlay(BUTTON + j));
        x += bw + 8.0;
    }
    let mut x = r.right() - PAD;
    for j in (3..BUTTONS.len()).rev() {
        let (label, chord, kind) = labels[j];
        let bw = widths[j];
        x -= bw;
        let ink = (BUTTONS[j] == Button::Stop && !s.stop_armed).then_some(t.danger);
        key_button(ui, Rect::new(x, by, bw, BUTTON_H), label, if chips { chord } else { "" }, kind, ink, s.button == BUTTONS[j], Action::Overlay(BUTTON + j));
        x -= 8.0;
    }
}

/// Where the watchdog stands: an icon and a word (reviewing, stood down, paused, watching) and what it's
/// about, the wake-ups it has used as segments with their count, and the line under it.
fn state(s: &WatchdogView, ui: &mut Ui, d: &WatchdogSnapshot, r: Rect, note: &TextBox) {
    let t = ui.t;
    let (word, detail) = match (&d.running, &d.finished, d.paused) {
        (Some(run), _, _) => ("reviewing", format!("{} · {}", run.reviewer, super::since(&run.started_at))),
        (None, Some(why), _) => ("stood down", why.clone()),
        (None, None, true) => ("paused", format!("{} works on without it", s.name)),
        (None, None, false) => ("watching", s.agent.clone()),
    };
    // the state: its icon (the spinner while a reviewer works), then the word
    let mid = Point::new(r.x + 8.0, r.y + 13.0);
    let mark = Rect::new(mid.x - 9.0, mid.y - 9.0, 18.0, 18.0);
    match (&d.running, &d.finished, d.paused) {
        (Some(_), _, _) => ui.spinner(mid, 5.0, 1.8, t.accent),
        (None, Some(_), _) => ui.icon(mark, CHECK_CIRCLE, 17.0, t.good),
        (None, None, true) => ui.icon(mark, PAUSE, 17.0, t.muted),
        (None, None, false) => ui.icon(mark, EYE, 17.0, t.good),
    }
    let st = Style::new(size::TITLE, weight::SEMIBOLD, t.text);
    let wb = ui.line(word, st, None);
    ui.draw(&wb, Point::new(r.x + 24.0, r.y + 13.0 - wb.h / 2.0));
    // the wake-ups at the right: segments for how much of the budget they've used, then the count
    let wst = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let wakes = format!("wake-up {} of {}", d.wakes, d.plan.max_wakes);
    let wwid = ui.measure(&wakes, wst);
    ui.label_in(Rect::new(r.right() - wwid, r.y + 3.0, wwid, 20.0), &wakes, wst, Align::Right);
    let sw = 72.0;
    let seg = Rect::new(r.right() - wwid - 12.0 - sw, r.y + 10.0, sw, 6.0);
    let used = if d.plan.max_wakes == 0 { 1.0 } else { d.wakes as f32 / d.plan.max_wakes as f32 };
    widgets::segments(ui, seg, used, widgets::segment_count(sw, 7.0), if used >= 0.9 { t.warn } else { t.accent });
    // what it's about, between the two (a reviewer or the agent it watches with its logo)
    let dx = r.x + 24.0 + wb.w + 12.0;
    let dw = seg.x - 16.0 - dx;
    if dw > 30.0 {
        marked(ui, Rect::new(dx, r.y + 3.0, dw, 20.0), &detail, Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
    }
    ui.draw(note, Point::new(r.x, r.y + 34.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_bridge::{Milestone, WatchdogPlan};
    use bro_core::config::WatchdogSettings;

    fn view(dog: Option<&WatchdogSnapshot>) -> WatchdogView {
        WatchdogView::new(1, "sid".into(), "Mal".into(), Some("Mal".into()), "codex · me".into(), dog, &WatchdogSettings::default(), "claude · home".into())
    }

    fn tap(v: &mut WatchdogView, n: usize, p: Point, dog: Option<&WatchdogSnapshot>) -> Out {
        click(v, n, p, KeyModifiers::NONE, 1, dog)
    }

    #[test]
    fn choices_keep_an_off_list_value_in_its_place() {
        assert_eq!(choices(&[20u64, 60, 120], 90), (vec![20, 60, 90, 120], 2));
        assert_eq!(choices(&[20u64, 60, 120], 60), (vec![20, 60, 120], 1));
    }

    #[test]
    fn agents_are_known_by_their_first_word() {
        assert_eq!(agent_mark("claude · work · opus"), Some(brand::harness(Harness::Claude)));
        assert_eq!(agent_mark("codex · personal · gpt-5.5"), Some(brand::harness(Harness::Codex)));
        assert_eq!(agent_mark("pi"), Some(brand::harness(Harness::Pi)));
        assert_eq!(agent_mark("this session stood down"), None);
        assert_eq!(agent_mark(""), None);
    }

    #[test]
    fn clicks_land_between_characters_and_stay_on_wrapped_rows() {
        // at 10 cells: "ship the ", "offline ", "sync"
        let ta = TextArea::new("ship the offline sync");
        let g = Geo { origin: Point::new(0.0, 0.0), width: 10, adv: 10.0, view: Rect::new(0.0, 0.0, 100.0, 60.0) };
        assert_eq!(byte_at(&ta, &g, Point::new(14.0, 5.0)), 1);
        assert_eq!(byte_at(&ta, &g, Point::new(999.0, 5.0)), 8, "past a wrapped row's end: before its trailing space");
        assert_eq!(byte_at(&ta, &g, Point::new(999.0, 45.0)), 21);
        assert_eq!(byte_at(&ta, &g, Point::new(0.0, 500.0)), 17, "below the rows in view: the last one");
    }

    #[test]
    fn clicks_do_what_the_terminal_ui_does() {
        let mut v = view(None);
        // the primary button answers the first click: nothing to start yet
        assert_eq!(tap(&mut v, PRIMARY, Point::default(), None), Out::None);
        assert!(v.error.is_some());
        assert_eq!(v.focus, Field::Milestone(0));
        // a choice sets its value and focuses its row
        MEMO.with(|c| c.borrow_mut().wake = vec![20, 60, 120, 300]);
        tap(&mut v, WAKE + 3, Point::default(), None);
        assert_eq!((v.idle_secs, v.focus), (300, Field::Wake));
        // a switch flips
        tap(&mut v, SWITCH + 1, Point::default(), None);
        assert!(!v.fresh);
        // a row: the first click focuses it, the next one changes it
        tap(&mut v, ROW + 3, Point::default(), None);
        assert_eq!((v.focus, v.max_wakes), (Field::Budget, 25));
        tap(&mut v, ROW + 3, Point::default(), None);
        assert_eq!(v.max_wakes, 50);
        // a milestone takes the caret where it was clicked
        v.milestones[0].set_text("offline sync");
        MEMO.with(|c| c.borrow_mut().fields = vec![Some(Geo { origin: Point::new(0.0, 0.0), width: 40, adv: 10.0, view: Rect::new(0.0, 0.0, 400.0, 20.0) })]);
        tap(&mut v, MILESTONE, Point::new(31.0, 5.0), None);
        assert_eq!((v.focus, v.milestones[0].cursor()), (Field::Milestone(0), 3));
        match tap(&mut v, PRIMARY, Point::default(), None) {
            Out::Start(plan) => assert_eq!((plan.idle_secs, plan.max_wakes, plan.fresh_context), (300, 50, false)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_new_plan_says_the_orchestrator_reviews() {
        let v = view(None);
        let new = form_intro(&v, false);
        assert!(new.contains("bro wakes the orchestrator") && new.contains("hands Mal its next task"), "{new}");
        assert!(!new.contains("fresh reviewer"), "{new}");
        // editing keeps the terminal UI's words
        assert_eq!(form_intro(&v, true), super::super::intro(&v, true));
    }

    #[test]
    fn the_body_follows_the_focus_and_shows_the_end_on_the_buttons() {
        let mut v = view(None);
        let blocks = vec![
            (40.0, Block::Watches),
            (32.0, Block::Section("Milestones", String::new())),
            (44.0, Block::Milestone(0)),
            (OPT_H, Block::Opt(0)),
            (OPT_H, Block::Opt(1)),
            (OPT_H, Block::Opt(2)),
            (OPT_H, Block::Opt(3)),
            (OPT_H, Block::Reviewer),
        ];
        // the budget (284..340) comes into a 200 px view, with a little room under it
        v.focus = Field::Budget;
        assert_eq!(follow(&v, &blocks, (0, 0), 0.0, 200.0), 148.0);
        // back up to the milestone: its caret row (80..100) shows with the field's padding
        v.focus = Field::Milestone(0);
        assert_eq!(follow(&v, &blocks, (0, 0), 148.0, 200.0), 60.0);
        // already in view: it stays
        assert_eq!(follow(&v, &blocks, (0, 0), 30.0, 200.0), 30.0);
        // the buttons show the end of the form (clamped by the body)
        v.focus = Field::Cancel;
        assert_eq!(follow(&v, &blocks, (0, 0), 0.0, 200.0), f32::MAX);
    }

    #[test]
    fn stop_takes_a_second_click() {
        let dog = WatchdogSnapshot { plan: WatchdogPlan { milestones: vec![Milestone::new("a")], ..WatchdogPlan::default() }, ..WatchdogSnapshot::default() };
        let mut v = view(Some(&dog));
        let stop = BUTTON + BUTTONS.iter().position(|b| *b == Button::Stop).unwrap();
        assert_eq!(tap(&mut v, stop, Point::default(), Some(&dog)), Out::None);
        assert!(v.stop_armed);
        assert_eq!(tap(&mut v, stop, Point::default(), Some(&dog)), Out::Stop);
        // another button disarms it
        let wake = BUTTON + BUTTONS.iter().position(|b| *b == Button::Wake).unwrap();
        tap(&mut v, stop, Point::default(), Some(&dog));
        assert_eq!(tap(&mut v, wake, Point::default(), Some(&dog)), Out::Wake);
        assert!(!v.stop_armed);
    }
}
