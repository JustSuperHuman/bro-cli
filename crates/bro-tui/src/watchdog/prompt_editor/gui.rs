//! The watchdog prompt editor in the desktop app: bro's standing instructions for every reviewer, edited in
//! the terminal font (the `TextArea` wraps by cells, so its width in cells comes from this layout). Keys
//! reach [`PromptEditor`] through the workspace; this draws its state and adds the mouse: the caret goes
//! where you click, the wheel scrolls, and the key hints are buttons that do what their keys do.
//!
//! A WinUI dialog like the watchdog's: the text in a multi-line text box, the buttons in the command band.

use super::{Out, PromptEditor};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::watchdog::view::gui::{self as shared, BAND_PAD, BUTTON_H, Caret, Geo, MONO, Opened, PAD, ROW_H};
use bro_bridge::watchdog::DEFAULT_PROMPT;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cell::RefCell;

// click targets
const SAVE: usize = 1;
const DEFAULT: usize = 2;
const CLOSE: usize = 3;
const TEXT: usize = 4;

/// Inside the editor's text box.
const WELL_X: f32 = 16.0;
const WELL_Y: f32 = 12.0;

/// What the editor remembers between frames (layout only: the text and cursor are the editor's own).
#[derive(Default)]
struct Memo {
    open: Opened,
    /// first visual row shown (the wheel moves it; the view follows the caret when it moves)
    top: usize,
    /// the cursor, text length and width last frame
    sig: Option<(usize, usize, usize)>,
    /// rows and rows in view last frame (the wheel stays inside them)
    rows: usize,
    room: usize,
    /// where the text sits, for clicks
    geo: Option<Geo>,
    caret: Caret,
}

thread_local! {
    static MEMO: RefCell<Memo> = RefCell::new(Memo::default());
}

/// Draw the prompt editor over the window (`area`: the whole window).
pub(crate) fn draw(s: &mut PromptEditor, ui: &mut Ui, area: Rect, cx: &Cx) -> bool {
    if area.w < 420.0 || area.h < 300.0 {
        return false;
    }
    let mut m = MEMO.with(|c| c.take());
    // still the size it was made with, after a while without a frame: a new editor at the old one's address
    let new = (s.width, s.height) == (80, 20) && m.open.idle(ui);
    if m.open.frame(ui, &*s as *const PromptEditor as usize, new) {
        m = Memo { open: m.open, ..Memo::default() };
    }
    let k = m.open.amount(ui);
    let t = ui.t;
    let w = (area.w - 48.0).min(1100.0);
    let r = shared::card(ui, area, &m.open, w, area.h - 48.0, 0.0, k);
    shared::title(ui, r, "watchdog prompt", None);
    let x0 = r.x + PAD;
    let cw = r.w - 2.0 * PAD;
    let mut y = r.y + shared::HEAD_H;

    // what it is, and whether it's bro's own (words at the right, as the terminal UI has them)
    let custom = s.area.text().trim() != DEFAULT_PROMPT.trim();
    let (tag, tc) = if custom { ("customized", t.shine) } else { ("bro's default", t.muted) };
    let tst = Style::new(size::SMALL, weight::MEDIUM, tc);
    let tw = ui.measure(tag, tst);
    ui.label_in(Rect::new(x0 + cw - tw, y, tw, 20.0), tag, tst, Align::Right);
    let about = "Every reviewer starts with these instructions, then gets a brief: the session, its milestones and the log.";
    let ab = ui.para(about, Style::new(size::BODY, weight::REGULAR, t.muted), cw - tw - 16.0, Some(2));
    ui.draw(&ab, Point::new(x0, y + (20.0 - ab.h).max(0.0) / 2.0));
    y += ab.h.max(20.0) + 2.0;
    // saved or not (the alert icon when esc would throw changes away)
    let (status, sc, wt) = if s.discard_armed {
        ("unsaved changes · esc again discards them", t.danger, weight::MEDIUM)
    } else if s.dirty() {
        ("unsaved changes", t.inline, weight::MEDIUM)
    } else {
        ("applies from the next wake-up", t.muted, weight::REGULAR)
    };
    if s.discard_armed {
        ui.icon(Rect::new(x0 - 1.0, y, 16.0, 18.0), "\u{F0026}", 13.0, sc);
        ui.label_in(Rect::new(x0 + 20.0, y, cw - 20.0, 18.0), status, Style::new(size::SMALL, wt, sc), Align::Left);
    } else {
        ui.label_in(Rect::new(x0, y, cw, 18.0), status, Style::new(size::SMALL, wt, sc), Align::Left);
    }
    y += 18.0 + 12.0;

    // the command band: bro's default at the left, close and save at the right
    let foot = shared::band(ui, r, 2.0 * BAND_PAD + BUTTON_H);
    let by = foot.y + BAND_PAD;
    let sw = shared::key_button_width(ui, "Save", "ctrl+s");
    let clw = shared::key_button_width(ui, "Close", "esc");
    let dw = shared::key_button_width(ui, "bro's default", "ctrl+r");
    let save = Rect::new(r.right() - PAD - sw, by, sw, BUTTON_H);
    let close = Rect::new(save.x - 8.0 - clw, by, clw, BUTTON_H);
    shared::key_button(ui, Rect::new(x0, by, dw, BUTTON_H), "bro's default", "ctrl+r", widgets::Kind::Secondary, None, false, Action::Overlay(DEFAULT));
    let close_kind = if s.discard_armed { widgets::Kind::Danger } else { widgets::Kind::Secondary };
    shared::key_button(ui, close, "Close", "esc", close_kind, None, false, Action::Overlay(CLOSE));
    shared::key_button(ui, save, "Save", "ctrl+s", widgets::Kind::Primary, None, false, Action::Overlay(SAVE));

    // the editor: a multi-line text box with the text in the terminal font, as tall as a whole number of rows;
    // it has the keys while the window does (the accent all round, as a focused field)
    let room = (((foot.y - 20.0 - y - 2.0 * WELL_Y) / ROW_H).floor() as usize).max(1);
    let well = Rect::new(x0, y, cw, room as f32 * ROW_H + 2.0 * WELL_Y);
    let hot = ui.hover_amount("watchdog-prompt-well", well);
    if cx.focused {
        ui.panel(well, radius::CONTROL, t.field_focus, t.accent.with_alpha(0.8));
    } else {
        ui.panel(well, radius::CONTROL, t.control.mix(t.control_hover, hot), t.stroke);
        let edge = Rect::new(well.x + radius::CONTROL, well.y, well.w - 2.0 * radius::CONTROL, well.h);
        ui.hline(edge.x, well.bottom() - ui.hair(), edge.w, t.muted.with_alpha(0.45));
    }
    let inner = Rect::new(well.x + WELL_X, well.y + WELL_Y, well.w - 2.0 * WELL_X - 6.0, room as f32 * ROW_H);
    let adv = shared::mono_advance(ui, MONO);
    let width = ((inner.w - 2.0) / adv).floor().max(4.0) as usize;
    // the keys move by these (PgUp / PgDn go a page of them)
    s.width = width;
    s.height = room;
    let rows = s.area.rows(width);
    let cur = s.area.cursor();
    let crow = shared::cursor_row(&rows, cur);
    let sig = (cur, s.area.text().len(), width);
    if m.sig != Some(sig) {
        // the caret moved or the text changed: keep it in view
        m.sig = Some(sig);
        if crow < m.top {
            m.top = crow;
        } else if crow >= m.top + room {
            m.top = crow + 1 - room;
        }
    }
    m.top = m.top.min(rows.len().saturating_sub(room));
    m.rows = rows.len();
    m.room = room;
    let off = ui.ease(("watchdog-prompt-scroll", m.open.key()), m.top as f32 * ROW_H, 90.0);
    let origin = Point::new(inner.x, inner.y - off);
    m.geo = Some(Geo { origin, width, adv, view: inner });
    let caret = cx.focused.then(|| m.caret.on(ui, cur, s.area.text().len()));
    // rows scroll under the well's edges, not into its padding
    ui.clip(Rect::new(well.x + 1.0, inner.y, well.w - 2.0, inner.h), 0.0);
    if s.area.text().is_empty() {
        let st = Style { italic: true, ..Style::new(size::BODY, weight::REGULAR, t.faint) };
        let pb = ui.line("empty: bro's own instructions are used", st, Some(inner.w));
        ui.draw(&pb, Point::new(inner.x, inner.y + (ROW_H - pb.h) / 2.0));
        if caret == Some(true) {
            shared::beam(ui, Point::new(inner.x, inner.y + 2.0));
        }
    } else {
        // the caret's line, softly
        let cy = origin.y + crow as f32 * ROW_H;
        ui.fill(Rect::new(well.x + 1.0, cy, well.w - 2.0, ROW_H), t.text.with_alpha(0.035));
        let first = ((off / ROW_H).floor() as usize).saturating_sub(1);
        let count = room + 3;
        // markdown headings stand out a little
        let heading = Style::mono(MONO, t.accent.mix(t.text, 0.3)).weight(weight::BOLD);
        shared::draw_rows(ui, &s.area, &rows, first, count, origin, Style::mono(MONO, t.text), Some(heading), caret);
    }
    ui.unclip();
    ui.hit_with(well, Action::Overlay(TEXT), Cursor::Text);
    let vis = if hot > 0.0 { 0.55 + 0.45 * hot } else { 0.55 };
    widgets::scrollbar(ui, Rect::new(well.x, well.y + 4.0, well.w - 4.0, well.h - 8.0), rows.len() as f32 * ROW_H, room as f32 * ROW_H, off, vis);
    MEMO.with(|c| c.replace(m));
    true
}

/// Target `n` was clicked: the buttons do what their keys do; a click in the text puts the caret there.
pub(crate) fn click(s: &mut PromptEditor, n: usize, p: Point, _mods: KeyModifiers, _count: u32) -> Out {
    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    match n {
        SAVE => s.key(ctrl('s')),
        DEFAULT => s.key(ctrl('r')),
        // as esc: with unsaved changes the first press warns
        CLOSE => s.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        TEXT => {
            if let Some(g) = MEMO.with(|c| c.borrow().geo) {
                let at = shared::byte_at(&s.area, &g, p);
                s.area.set_cursor(at);
                s.discard_armed = false;
            }
            Out::None
        }
        _ => Out::None,
    }
}

/// The wheel scrolls the text (the caret stays where it is).
pub(crate) fn wheel(_s: &mut PromptEditor, _n: Option<usize>, down: bool) {
    MEMO.with(|c| {
        let mut m = c.borrow_mut();
        let max = m.rows.saturating_sub(m.room);
        m.top = if down { (m.top + 3).min(max) } else { m.top.saturating_sub(3) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(e: &mut PromptEditor, n: usize, p: Point) -> Out {
        click(e, n, p, KeyModifiers::NONE, 1)
    }

    #[test]
    fn buttons_do_what_their_keys_do() {
        let mut e = PromptEditor::new(None);
        assert_eq!(tap(&mut e, SAVE, Point::default()), Out::Save(None), "bro's own saves as none");
        e.paste("Always run the tests first.\n");
        assert_eq!(tap(&mut e, CLOSE, Point::default()), Out::None, "unsaved: the first close warns");
        assert!(e.discard_armed);
        // a click in the text puts the caret there, and disarms
        MEMO.with(|c| c.borrow_mut().geo = Some(Geo { origin: Point::new(0.0, 0.0), width: 200, adv: 10.0, view: Rect::new(0.0, 0.0, 2000.0, 400.0) }));
        tap(&mut e, TEXT, Point::new(21.0, 5.0));
        assert_eq!(e.area.cursor(), 2);
        assert!(!e.discard_armed);
        assert_eq!(tap(&mut e, CLOSE, Point::default()), Out::None);
        assert_eq!(tap(&mut e, CLOSE, Point::default()), Out::Close);
        tap(&mut e, DEFAULT, Point::default());
        assert!(e.area.text().starts_with("You are bro's watchdog"));
    }
}
